use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Per-project configuration, loaded from `.scip-io.toml` if present.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Override which languages to index
    #[serde(default)]
    pub languages: Vec<String>,

    /// Override output path
    pub output: Option<PathBuf>,

    /// Per-language indexer overrides
    #[serde(default)]
    pub indexer: std::collections::HashMap<String, IndexerOverride>,

    /// Global settings (parallel, timeout, cache)
    pub settings: Option<Settings>,

    /// Monorepo sub-project entries
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,

    /// Merge configuration
    pub merge: Option<MergeConfig>,
}

/// Per-language overrides for indexer config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerOverride {
    /// Custom binary path (skip download)
    pub binary: Option<PathBuf>,
    /// Override CLI arguments
    pub args: Option<Vec<String>>,
    /// Override version
    pub version: Option<String>,
    /// Whether this indexer is enabled (default: true)
    pub enabled: Option<bool>,
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
        assert!(config.indexer.is_empty());
        assert!(config.settings.is_none());
        assert!(config.projects.is_empty());
        assert!(config.merge.is_none());
    }

    #[test]
    fn test_load_basic_config() {
        let dir = TempDir::new().unwrap();
        let config_content = r#"
            languages = ["typescript", "python"]
            output = "build/index.scip"
        "#;
        fs::write(dir.path().join(".scip-io.toml"), config_content).unwrap();
        let config = ProjectConfig::load(dir.path()).unwrap();
        assert_eq!(config.languages, vec!["typescript", "python"]);
        assert_eq!(
            config.output.unwrap(),
            PathBuf::from("build/index.scip")
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
        assert!(config.indexer.is_empty());
    }

    #[test]
    fn test_config_default_impl() {
        let config = ProjectConfig::default();
        assert!(config.languages.is_empty());
        assert!(config.output.is_none());
        assert!(config.indexer.is_empty());
        assert!(config.settings.is_none());
        assert!(config.projects.is_empty());
        assert!(config.merge.is_none());
    }

    #[test]
    fn test_config_roundtrip_serialization() {
        let mut config = ProjectConfig::default();
        config.languages = vec!["rust".into(), "go".into()];
        config.output = Some(PathBuf::from("out.scip"));

        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: ProjectConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.languages, vec!["rust", "go"]);
        assert_eq!(deserialized.output.unwrap(), PathBuf::from("out.scip"));
    }
}
