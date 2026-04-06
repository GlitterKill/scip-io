use serde::{Deserialize, Serialize};

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

    /// Return true if the given filename is a manifest file for this language.
    pub fn matches_manifest(self, filename: &str) -> bool {
        match self {
            Self::TypeScript => filename == "tsconfig.json",
            Self::JavaScript => filename == "package.json",
            Self::Python => {
                matches!(
                    filename,
                    "pyproject.toml"
                        | "setup.py"
                        | "setup.cfg"
                        | "requirements.txt"
                        | "Pipfile"
                )
            }
            Self::Rust => filename == "Cargo.toml",
            Self::Go => filename == "go.mod",
            Self::Java => filename == "pom.xml" || filename == "build.gradle",
            Self::CSharp => filename.ends_with(".csproj") || filename.ends_with(".sln"),
            Self::Ruby => filename == "Gemfile",
            Self::Kotlin => {
                filename == "build.gradle.kts" || filename == "settings.gradle.kts"
            }
            Self::Cpp => filename == "CMakeLists.txt" || filename == "compile_commands.json",
            Self::Scala => filename == "build.sbt",
        }
    }

    pub fn with_evidence(self, evidence: String) -> Language {
        Language {
            kind: self,
            evidence,
        }
    }
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
    }

    #[test]
    fn test_manifest_detection_typescript() {
        assert!(LanguageKind::TypeScript.matches_manifest("tsconfig.json"));
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
            assert!(!kind.matches_manifest("Makefile"));
        }
    }

    #[test]
    fn test_with_evidence_constructs_language() {
        let lang = LanguageKind::Go.with_evidence("go.mod".into());
        assert_eq!(lang.kind, LanguageKind::Go);
        assert_eq!(lang.evidence, "go.mod");
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
