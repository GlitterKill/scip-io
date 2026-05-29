use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Relative strength of the file that proved a language is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DetectionEvidenceKind {
    SourceFile,
    BuildFile,
    ProjectConfig,
}

impl DetectionEvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceFile => "source_file",
            Self::BuildFile => "build_file",
            Self::ProjectConfig => "project_config",
        }
    }
}

/// A programming language that SCIP-IO can detect and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LanguageKind {
    TypeScript,
    JavaScript,
    Python,
    Rust,
    Go,
    Java,
    CSharp,
    Ruby,
    Kotlin,
    Cpp,
    Scala,
}

/// A detected language with evidence of why it was detected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Language {
    pub kind: LanguageKind,
    pub evidence: String,
    #[serde(default)]
    pub evidence_kind: String,
    #[serde(default = "default_indexer_ready")]
    pub indexer_ready: bool,
    #[serde(default)]
    pub readiness_message: Option<String>,
    #[serde(default)]
    pub additional_configs: Vec<PathBuf>,
}

fn default_indexer_ready() -> bool {
    true
}

impl Language {
    /// All language kinds we scan for, in priority order.
    pub const ALL: &[LanguageKind] = &[
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

    pub fn name(&self) -> &'static str {
        self.kind.name()
    }

    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

impl LanguageKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Ruby => "ruby",
            Self::Kotlin => "kotlin",
            Self::Cpp => "cpp",
            Self::Scala => "scala",
        }
    }

    /// Return true if the given filename is a manifest/config file for this language.
    pub fn matches_manifest(self, filename: &str) -> bool {
        match self {
            Self::TypeScript => filename == "tsconfig.json",
            Self::JavaScript => filename == "package.json",
            Self::Python => {
                matches!(
                    filename,
                    "pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt" | "Pipfile"
                )
            }
            Self::Rust => filename == "Cargo.toml",
            Self::Go => filename == "go.mod",
            Self::Java => filename == "pom.xml" || filename == "build.gradle",
            Self::CSharp => {
                filename.ends_with(".csproj")
                    || filename.ends_with(".sln")
                    || filename.ends_with(".vbproj")
            }
            Self::Ruby => filename == "Gemfile",
            Self::Kotlin => filename == "build.gradle.kts" || filename == "settings.gradle.kts",
            Self::Cpp => filename == "CMakeLists.txt" || filename == "compile_commands.json",
            Self::Scala => filename == "build.sbt",
        }
    }

    /// Return the evidence kind if a filename proves this language is present.
    pub fn detect_evidence(self, filename: &str) -> Option<DetectionEvidenceKind> {
        if self.matches_project_config(filename) {
            return Some(DetectionEvidenceKind::ProjectConfig);
        }
        if self.matches_build_file(filename) {
            return Some(DetectionEvidenceKind::BuildFile);
        }
        if self.matches_source_file(filename) {
            return Some(DetectionEvidenceKind::SourceFile);
        }
        None
    }

    fn matches_project_config(self, filename: &str) -> bool {
        match self {
            Self::TypeScript => is_tsconfig(filename),
            Self::JavaScript => filename == "package.json",
            Self::Python => {
                matches!(
                    filename,
                    "pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt" | "Pipfile"
                )
            }
            Self::Rust => filename == "Cargo.toml" || filename == "rust-project.json",
            Self::Go => filename == "go.mod",
            Self::Java => filename == "pom.xml",
            Self::CSharp => {
                filename.ends_with(".csproj")
                    || filename.ends_with(".sln")
                    || filename.ends_with(".vbproj")
            }
            Self::Ruby => filename == "Gemfile",
            Self::Kotlin => filename == "build.gradle.kts" || filename == "settings.gradle.kts",
            Self::Cpp => filename == "compile_commands.json",
            Self::Scala => filename == "build.sbt",
        }
    }

    fn matches_build_file(self, filename: &str) -> bool {
        match self {
            Self::Java => filename == "build.gradle",
            Self::Cpp => {
                filename == "CMakeLists.txt"
                    || filename == "Makefile"
                    || filename.starts_with("Makefile.")
                    || filename == "Kbuild"
                    || filename.starts_with("Kbuild.")
                    || filename == "Kconfig"
                    || filename.starts_with("Kconfig.")
            }
            _ => false,
        }
    }

    fn matches_source_file(self, filename: &str) -> bool {
        let extension = filename
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase());

        match self {
            Self::TypeScript => matches!(extension.as_deref(), Some("ts" | "tsx")),
            Self::JavaScript => matches!(extension.as_deref(), Some("js" | "jsx" | "mjs" | "cjs")),
            Self::Python => matches!(extension.as_deref(), Some("py" | "pyw")),
            Self::Rust => matches!(extension.as_deref(), Some("rs")),
            Self::Go => matches!(extension.as_deref(), Some("go")),
            Self::Java => matches!(extension.as_deref(), Some("java")),
            Self::CSharp => matches!(extension.as_deref(), Some("cs")),
            Self::Ruby => matches!(extension.as_deref(), Some("rb")),
            Self::Kotlin => matches!(extension.as_deref(), Some("kt" | "kts")),
            Self::Cpp => {
                matches!(
                    extension.as_deref(),
                    Some("c" | "h" | "cc" | "hh" | "cpp" | "hpp" | "cxx" | "hxx" | "s")
                )
            }
            Self::Scala => matches!(extension.as_deref(), Some("scala" | "sbt")),
        }
    }

    pub fn with_evidence(self, evidence: String) -> Language {
        self.with_detected_evidence(evidence, DetectionEvidenceKind::ProjectConfig)
    }

    pub fn with_detected_evidence(
        self,
        evidence: String,
        evidence_kind: DetectionEvidenceKind,
    ) -> Language {
        let (indexer_ready, readiness_message) = self.indexer_readiness(evidence_kind, &evidence);
        Language {
            kind: self,
            evidence,
            evidence_kind: evidence_kind.as_str().to_string(),
            indexer_ready,
            readiness_message,
            additional_configs: Vec::new(),
        }
    }

    pub fn indexer_readiness(
        self,
        evidence_kind: DetectionEvidenceKind,
        evidence: &str,
    ) -> (bool, Option<String>) {
        match self {
            Self::Rust if evidence_kind != DetectionEvidenceKind::ProjectConfig => (
                false,
                Some(
                    "rust-analyzer SCIP indexing needs Cargo.toml or rust-project.json; \
                     non-Cargo projects may need to generate rust-project.json first."
                        .to_string(),
                ),
            ),
            Self::Cpp if evidence_file_name(evidence) != "compile_commands.json" => (
                false,
                Some(
                    "scip-clang indexing needs compile_commands.json; CMake, Makefile, \
                     Kbuild, Kconfig, and source files only prove C/C++ is present."
                        .to_string(),
                ),
            ),
            Self::Cpp if evidence_has_parent_components(evidence) => (
                false,
                Some(
                    "scip-clang indexes the selected project root with compile_commands.json; \
                     a nested compile database proves C/C++ is present but does not make \
                     the parent root directly index-ready."
                        .to_string(),
                ),
            ),
            _ => (true, None),
        }
    }
}

fn is_tsconfig(filename: &str) -> bool {
    filename == "tsconfig.json" || filename.starts_with("tsconfig.") && filename.ends_with(".json")
}

fn evidence_file_name(evidence: &str) -> &str {
    evidence.rsplit(['/', '\\']).next().unwrap_or(evidence)
}

fn evidence_has_parent_components(evidence: &str) -> bool {
    evidence.contains('/') || evidence.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_kind_name_all_variants() {
        assert_eq!(LanguageKind::TypeScript.name(), "typescript");
        assert_eq!(LanguageKind::JavaScript.name(), "javascript");
        assert_eq!(LanguageKind::Python.name(), "python");
        assert_eq!(LanguageKind::Rust.name(), "rust");
        assert_eq!(LanguageKind::Go.name(), "go");
        assert_eq!(LanguageKind::Java.name(), "java");
        assert_eq!(LanguageKind::CSharp.name(), "csharp");
        assert_eq!(LanguageKind::Ruby.name(), "ruby");
        assert_eq!(LanguageKind::Kotlin.name(), "kotlin");
        assert_eq!(LanguageKind::Cpp.name(), "cpp");
        assert_eq!(LanguageKind::Scala.name(), "scala");
    }

    #[test]
    fn test_language_name_delegates_to_kind() {
        let lang = LanguageKind::Rust.with_evidence("Cargo.toml".into());
        assert_eq!(lang.name(), "rust");
    }

    #[test]
    fn test_language_evidence() {
        let lang = LanguageKind::Python.with_evidence("pyproject.toml".into());
        assert_eq!(lang.evidence(), "pyproject.toml");
        assert_eq!(lang.evidence_kind, "project_config");
        assert!(lang.indexer_ready);
    }

    #[test]
    fn test_manifest_detection_typescript() {
        assert!(LanguageKind::TypeScript.matches_manifest("tsconfig.json"));
        assert!(
            LanguageKind::TypeScript
                .detect_evidence("tsconfig.app.json")
                .is_some()
        );
        assert!(!LanguageKind::TypeScript.matches_manifest("package.json"));
    }

    #[test]
    fn test_manifest_detection_javascript() {
        assert!(LanguageKind::JavaScript.matches_manifest("package.json"));
        assert!(!LanguageKind::JavaScript.matches_manifest("tsconfig.json"));
    }

    #[test]
    fn test_manifest_detection_python() {
        assert!(LanguageKind::Python.matches_manifest("pyproject.toml"));
        assert!(LanguageKind::Python.matches_manifest("setup.py"));
        assert!(LanguageKind::Python.matches_manifest("setup.cfg"));
        assert!(LanguageKind::Python.matches_manifest("requirements.txt"));
        assert!(LanguageKind::Python.matches_manifest("Pipfile"));
        assert!(!LanguageKind::Python.matches_manifest("Cargo.toml"));
    }

    #[test]
    fn test_manifest_detection_rust() {
        assert!(LanguageKind::Rust.matches_manifest("Cargo.toml"));
        assert!(
            LanguageKind::Rust
                .detect_evidence("rust-project.json")
                .is_some()
        );
        assert!(!LanguageKind::Rust.matches_manifest("go.mod"));
    }

    #[test]
    fn test_manifest_detection_go() {
        assert!(LanguageKind::Go.matches_manifest("go.mod"));
        assert!(!LanguageKind::Go.matches_manifest("Cargo.toml"));
    }

    #[test]
    fn test_manifest_detection_java() {
        assert!(LanguageKind::Java.matches_manifest("pom.xml"));
        assert!(LanguageKind::Java.matches_manifest("build.gradle"));
        assert!(!LanguageKind::Java.matches_manifest("build.gradle.kts"));
    }

    #[test]
    fn test_manifest_detection_csharp() {
        assert!(LanguageKind::CSharp.matches_manifest("MyApp.csproj"));
        assert!(LanguageKind::CSharp.matches_manifest("Solution.sln"));
        assert!(LanguageKind::CSharp.matches_manifest("Legacy.vbproj"));
        assert!(!LanguageKind::CSharp.matches_manifest("pom.xml"));
    }

    #[test]
    fn test_manifest_detection_ruby() {
        assert!(LanguageKind::Ruby.matches_manifest("Gemfile"));
        assert!(!LanguageKind::Ruby.matches_manifest("Pipfile"));
    }

    #[test]
    fn test_manifest_detection_kotlin() {
        assert!(LanguageKind::Kotlin.matches_manifest("build.gradle.kts"));
        assert!(LanguageKind::Kotlin.matches_manifest("settings.gradle.kts"));
        assert!(!LanguageKind::Kotlin.matches_manifest("build.gradle"));
    }

    #[test]
    fn test_manifest_detection_cpp() {
        assert!(LanguageKind::Cpp.matches_manifest("CMakeLists.txt"));
        assert!(LanguageKind::Cpp.matches_manifest("compile_commands.json"));
        assert!(LanguageKind::Cpp.detect_evidence("Makefile").is_some());
        assert!(LanguageKind::Cpp.detect_evidence("Kbuild").is_some());
        assert!(LanguageKind::Cpp.detect_evidence("Kconfig").is_some());
        assert!(!LanguageKind::Cpp.matches_manifest("Cargo.toml"));
    }

    #[test]
    fn test_manifest_detection_scala() {
        assert!(LanguageKind::Scala.matches_manifest("build.sbt"));
        assert!(!LanguageKind::Scala.matches_manifest("pom.xml"));
    }
    #[test]
    fn test_all_languages_constant() {
        assert_eq!(Language::ALL.len(), 11);
        // Verify order matches declaration
        assert_eq!(Language::ALL[0], LanguageKind::TypeScript);
        assert_eq!(Language::ALL[9], LanguageKind::Cpp);
        assert_eq!(Language::ALL[10], LanguageKind::Scala);
    }

    #[test]
    fn test_no_false_positive_manifests() {
        // Random filenames should not match any language
        for kind in Language::ALL {
            assert!(!kind.matches_manifest("README.md"));
            assert!(!kind.matches_manifest("index.ts"));
            assert!(!kind.matches_manifest("main.py"));
        }
    }

    #[test]
    fn test_with_evidence_constructs_language() {
        let lang = LanguageKind::Go.with_evidence("go.mod".into());
        assert_eq!(lang.kind, LanguageKind::Go);
        assert_eq!(lang.evidence, "go.mod");
        assert_eq!(lang.evidence_kind, "project_config");
    }

    #[test]
    fn test_language_kind_equality() {
        assert_eq!(LanguageKind::Rust, LanguageKind::Rust);
        assert_ne!(LanguageKind::Rust, LanguageKind::Go);
    }

    #[test]
    fn test_language_kind_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(LanguageKind::Python);
        set.insert(LanguageKind::Python);
        assert_eq!(set.len(), 1);
    }
}
