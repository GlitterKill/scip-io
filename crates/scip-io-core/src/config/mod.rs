use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::indexer::backend::{BackendPreference, ExecutionBackendKind};
use crate::toolchain::ToolchainsConfig;

/// Per-project configuration, loaded from `.scip-io.toml` if present.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Override which languages to index
    #[serde(default)]
    pub languages: Vec<String>,

    /// Override output path
    pub output: Option<PathBuf>,

    /// Include supported secondary config files during indexing
    pub include_additional_configs: Option<bool>,

    /// Controls whether indexing is scoped to the selected repository tree or
    /// to discovered language config/build roots.
    pub scope: Option<IndexScope>,

    /// Per-language indexer overrides
    #[serde(default)]
    pub indexer: std::collections::HashMap<String, IndexerOverride>,

    /// Global settings (parallel, timeout, cache)
    pub settings: Option<Settings>,

    /// Runtime toolchain homes used to build child-process environments.
    #[serde(default, skip_serializing_if = "ToolchainsConfig::is_empty")]
    pub toolchains: ToolchainsConfig,

    /// C/C++ language-specific settings.
    pub cpp: Option<CppConfig>,

    /// Monorepo sub-project entries
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,

    /// Merge configuration
    pub merge: Option<MergeConfig>,
}

/// C/C++ language-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CppConfig {
    /// CMake compile database generation settings.
    pub cmake: Option<CmakeCompileDatabaseConfig>,
}

/// Optional CMake configure jobs used to create compile database files before
/// C/C++ indexing discovers and merges them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CmakeCompileDatabaseConfig {
    /// Generate configured CMake build directories before indexing.
    pub generate_compile_databases: Option<bool>,
    /// Preset build matrix for known repository layouts.
    pub preset: Option<CmakeCompileDatabasePreset>,
    /// CMake executable path. Defaults to `cmake`.
    pub cmake: Option<PathBuf>,
    /// Shared source directory for configured or preset builds.
    pub source_dir: Option<PathBuf>,
    /// Shared build root for preset builds.
    pub build_root: Option<PathBuf>,
    /// Shared CMake generator, for example `Ninja`.
    pub generator: Option<String>,
    /// Re-run CMake even when the build dir already has compile_commands.json.
    pub refresh: Option<bool>,
    /// Additional custom CMake configure jobs.
    #[serde(default)]
    pub builds: Vec<CmakeCompileDatabaseBuildConfig>,
}

/// A single CMake configure job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmakeCompileDatabaseBuildConfig {
    /// Human-readable label used in logs and dry-run output.
    pub name: Option<String>,
    /// Source directory for this configure job.
    pub source_dir: Option<PathBuf>,
    /// Build directory that should receive compile_commands.json.
    pub build_dir: PathBuf,
    /// CMake generator for this configure job.
    pub generator: Option<String>,
    /// Re-run CMake even when compile_commands.json already exists.
    pub refresh: Option<bool>,
    /// Extra CMake configure arguments, normally `-D...` definitions.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Built-in CMake compile database matrix presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CmakeCompileDatabasePreset {
    /// LLVM-oriented broad coverage: all LLVM targets, broad projects, and runtimes.
    LlvmBroad,
}

impl fmt::Display for CmakeCompileDatabasePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LlvmBroad => "llvm-broad",
        })
    }
}

impl FromStr for CmakeCompileDatabasePreset {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "llvm-broad" | "llvm_broad" => Ok(Self::LlvmBroad),
            _ => Err(format!(
                "invalid CMake compile database preset '{value}', expected 'llvm-broad'"
            )),
        }
    }
}

/// Root selection mode for indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IndexScope {
    /// Index from the selected repository root and let each upstream indexer
    /// decide what it can cover within that tree.
    #[default]
    RepoTree,
    /// Preserve the legacy behavior: schedule discovered config/build roots as
    /// separate indexing runs and prune nested output ownership.
    Configs,
}

impl fmt::Display for IndexScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RepoTree => "repo-tree",
            Self::Configs => "configs",
        })
    }
}

impl FromStr for IndexScope {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "repo-tree" | "repo_tree" | "repo" => Ok(Self::RepoTree),
            "configs" | "config" | "legacy" => Ok(Self::Configs),
            _ => Err(format!(
                "invalid index scope '{value}', expected 'repo-tree' or 'configs'"
            )),
        }
    }
}

/// Per-language overrides for indexer config.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexerOverride {
    /// Custom binary path (skip download)
    pub binary: Option<PathBuf>,
    /// Override CLI arguments
    pub args: Option<Vec<String>>,
    /// Override version
    pub version: Option<String>,
    /// Whether this indexer is enabled (default: true)
    pub enabled: Option<bool>,
    /// Execution backend for Linux-only indexers on Windows.
    pub backend: Option<ExecutionBackendKind>,
    /// Docker image to use when `backend = "docker"`.
    pub docker_image: Option<String>,
    /// WSL distribution to use when `backend = "wsl"`.
    pub wsl_distro: Option<String>,
}

/// Global settings for the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Maximum number of parallel indexer invocations
    pub parallel: Option<u32>,
    /// Timeout in seconds for each indexer run
    pub timeout: Option<u64>,
    /// Custom cache directory for downloaded binaries
    pub cache_dir: Option<PathBuf>,
    /// Default execution backend for Linux-only indexers on Windows.
    pub linux_indexer_backend: Option<ExecutionBackendKind>,
}

/// A sub-project entry for monorepo support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    /// Relative path to the sub-project root
    pub path: PathBuf,
    /// Override which languages to index for this sub-project
    #[serde(default)]
    pub languages: Vec<String>,
    /// Override output path for this sub-project
    pub output: Option<PathBuf>,
}

/// Configuration for the merge step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeConfig {
    /// Whether merging is enabled (default: true)
    pub enabled: Option<bool>,
    /// Output path for the merged index
    pub output: Option<PathBuf>,
}

impl ProjectConfig {
    /// Load config from `.scip-io.toml` in the given directory, or return defaults.
    pub fn load(project_root: &std::path::Path) -> Result<Self> {
        let config_path = project_root.join(".scip-io.toml");
        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)?;
            let config: Self = toml::from_str(&contents)?;
            tracing::info!(path = %config_path.display(), "loaded project config");
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn backend_preference_for(&self, language: &str, indexer_name: &str) -> BackendPreference {
        let global_backend = self
            .settings
            .as_ref()
            .and_then(|settings| settings.linux_indexer_backend);
        let override_config = self
            .indexer
            .get(language)
            .or_else(|| self.indexer.get(indexer_name));

        BackendPreference {
            kind: override_config
                .and_then(|config| config.backend)
                .or(global_backend)
                .unwrap_or_default(),
            docker_image: override_config.and_then(|config| config.docker_image.clone()),
            wsl_distro: override_config.and_then(|config| config.wsl_distro.clone()),
        }
    }

    pub fn args_override_for(&self, language: &str, indexer_name: &str) -> Option<Vec<String>> {
        self.indexer
            .get(language)
            .or_else(|| self.indexer.get(indexer_name))
            .and_then(|config| config.args.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_missing_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        assert!(config.languages.is_empty());
        assert!(config.output.is_none());
        assert!(config.include_additional_configs.is_none());
        assert!(config.indexer.is_empty());
        assert!(config.settings.is_none());
        assert!(config.toolchains.is_empty());
        assert!(config.cpp.is_none());
        assert!(config.projects.is_empty());
        assert!(config.merge.is_none());
    }

    #[test]
    fn test_load_basic_config() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            languages = ["typescript", "python"]
            output = "build/index.scip"
            include_additional_configs = true
            scope = "configs"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        assert_eq!(config.languages, vec!["typescript", "python"]);
        assert_eq!(config.output.unwrap(), PathBuf::from("build/index.scip"));
        assert_eq!(config.include_additional_configs, Some(true));
        assert_eq!(config.scope, Some(IndexScope::Configs));
    }

    #[test]
    fn test_load_config_with_repo_tree_scope() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".scip-io.toml"), r#"scope = "repo-tree""#).unwrap();

        let config = ProjectConfig::load(dir.path()).unwrap();

        assert_eq!(config.scope, Some(IndexScope::RepoTree));
    }

    #[test]
    fn test_load_config_with_cmake_compile_database_generation() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [cpp.cmake]
            generate_compile_databases = true
            preset = "llvm-broad"
            generator = "Ninja"
            build_root = "out/scip-io-cmake"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();

        let config = ProjectConfig::load(dir.path()).unwrap();
        let cmake = config.cpp.unwrap().cmake.unwrap();

        assert_eq!(cmake.generate_compile_databases, Some(true));
        assert_eq!(cmake.preset, Some(CmakeCompileDatabasePreset::LlvmBroad));
        assert_eq!(cmake.generator.as_deref(), Some("Ninja"));
        assert_eq!(
            cmake.build_root.unwrap(),
            PathBuf::from("out/scip-io-cmake")
        );
    }

    #[test]
    fn test_load_config_with_indexer_overrides() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [indexer.typescript]
            args = ["index", "--custom-flag"]
            version = "0.3.10"
            enabled = false
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        let ts = config.indexer.get("typescript").unwrap();
        assert_eq!(
            ts.args.as_ref().unwrap(),
            &vec!["index".to_string(), "--custom-flag".to_string()]
        );
        assert_eq!(ts.version.as_deref(), Some("0.3.10"));
        assert_eq!(ts.enabled, Some(false));
        assert!(ts.binary.is_none());
    }

    #[test]
    fn test_load_config_with_custom_binary() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [indexer.rust]
            binary = "/usr/local/bin/rust-analyzer"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        let rust = config.indexer.get("rust").unwrap();
        assert_eq!(
            rust.binary.as_ref().unwrap(),
            &PathBuf::from("/usr/local/bin/rust-analyzer")
        );
    }

    #[test]
    fn test_load_config_with_settings() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [settings]
            parallel = 8
            timeout = 1200
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        let settings = config.settings.unwrap();
        assert_eq!(settings.parallel, Some(8));
        assert_eq!(settings.timeout, Some(1200));
        assert!(settings.cache_dir.is_none());
        assert!(settings.linux_indexer_backend.is_none());
    }

    #[test]
    fn test_load_config_with_cache_dir() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [settings]
            cache_dir = "/tmp/scip-io-cache"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        let settings = config.settings.unwrap();
        assert_eq!(
            settings.cache_dir.unwrap(),
            PathBuf::from("/tmp/scip-io-cache")
        );
    }

    #[test]
    fn config_backend_fields_parse_global_and_per_indexer_preferences() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [settings]
            linux_indexer_backend = "auto"

            [indexer.ruby]
            backend = "wsl"

            [indexer.cpp]
            backend = "docker"
            docker_image = "my/scip-clang-runtime:latest"
            wsl_distro = "Ubuntu-24.04"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();

        let config = ProjectConfig::load(dir.path()).unwrap();

        assert_eq!(
            config.settings.as_ref().unwrap().linux_indexer_backend,
            Some(ExecutionBackendKind::Auto)
        );
        let ruby = config.indexer.get("ruby").unwrap();
        assert_eq!(ruby.backend, Some(ExecutionBackendKind::Wsl));
        let cpp = config.indexer.get("cpp").unwrap();
        assert_eq!(cpp.backend, Some(ExecutionBackendKind::Docker));
        assert_eq!(
            cpp.docker_image.as_deref(),
            Some("my/scip-clang-runtime:latest")
        );
        assert_eq!(cpp.wsl_distro.as_deref(), Some("Ubuntu-24.04"));
        assert_eq!(
            config
                .backend_preference_for("cpp", "scip-clang")
                .docker_image
                .as_deref(),
            Some("my/scip-clang-runtime:latest")
        );
    }

    #[test]
    fn config_args_override_can_match_language_or_indexer_name() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [indexer.scala]
            args = ["index", "--", "-pl", "core", "-am"]

            [indexer."scip-java"]
            args = ["index", "--build-tool", "maven"]
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();

        let config = ProjectConfig::load(dir.path()).unwrap();

        assert_eq!(
            config.args_override_for("scala", "scip-java").unwrap(),
            vec!["index", "--", "-pl", "core", "-am"]
        );
        assert_eq!(
            config.args_override_for("java", "scip-java").unwrap(),
            vec!["index", "--build-tool", "maven"]
        );
    }

    #[test]
    fn config_toolchain_homes_parse_for_runtime_env_injection() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [toolchains.go]
            home = "C:\\Program Files\\Go"

            [toolchains.java]
            home = "C:\\Program Files\\Eclipse Adoptium\\jdk-21"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();

        let config = ProjectConfig::load(dir.path()).unwrap();

        assert_eq!(
            config
                .toolchains
                .go
                .as_ref()
                .and_then(|config| config.home.as_ref()),
            Some(&PathBuf::from("C:\\Program Files\\Go"))
        );
        assert_eq!(
            config
                .toolchains
                .java
                .as_ref()
                .and_then(|config| config.home.as_ref()),
            Some(&PathBuf::from(
                "C:\\Program Files\\Eclipse Adoptium\\jdk-21"
            ))
        );
    }

    #[test]
    fn test_load_config_with_monorepo_projects() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [[projects]]
            path = "services/api"
            languages = ["typescript"]
            output = "services/api/index.scip"

            [[projects]]
            path = "tools/cli"
            languages = ["rust"]
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        assert_eq!(config.projects.len(), 2);
        assert_eq!(config.projects[0].path, PathBuf::from("services/api"));
        assert_eq!(config.projects[0].languages, vec!["typescript"]);
        assert_eq!(
            config.projects[0].output.as_ref().unwrap(),
            &PathBuf::from("services/api/index.scip")
        );
        assert_eq!(config.projects[1].path, PathBuf::from("tools/cli"));
        assert_eq!(config.projects[1].languages, vec!["rust"]);
        assert!(config.projects[1].output.is_none());
    }

    #[test]
    fn test_load_config_with_merge_config() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            [merge]
            enabled = true
            output = "dist/index.scip"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        let merge = config.merge.unwrap();
        assert_eq!(merge.enabled, Some(true));
        assert_eq!(merge.output.unwrap(), PathBuf::from("dist/index.scip"));
    }

    #[test]
    fn test_invalid_config_returns_error() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".scip-io.toml"), "{{invalid toml").unwrap();
        assert!(ProjectConfig::load(dir.path()).is_err());
    }

    #[test]
    fn test_empty_config_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".scip-io.toml"), "").unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        assert!(config.languages.is_empty());
        assert!(config.output.is_none());
        assert!(config.include_additional_configs.is_none());
        assert!(config.indexer.is_empty());
        assert!(config.toolchains.is_empty());
    }

    #[test]
    fn test_config_default_impl() {
        let config = ProjectConfig::default();
        assert!(config.languages.is_empty());
        assert!(config.output.is_none());
        assert!(config.include_additional_configs.is_none());
        assert!(config.indexer.is_empty());
        assert!(config.settings.is_none());
        assert!(config.toolchains.is_empty());
        assert!(config.projects.is_empty());
        assert!(config.merge.is_none());
    }

    #[test]
    fn test_config_roundtrip_serialization() {
        let config = ProjectConfig {
            languages: vec!["rust".into(), "go".into()],
            output: Some(PathBuf::from("out.scip")),
            ..Default::default()
        };

        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: ProjectConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.languages, vec!["rust", "go"]);
        assert_eq!(deserialized.output.unwrap(), PathBuf::from("out.scip"));
    }
}
