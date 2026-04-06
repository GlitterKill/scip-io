pub mod install;
pub mod registry;
pub mod runner;

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::progress::{ProgressEvent, ProgressHandler};

/// How to install a particular SCIP indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InstallMethod {
    /// Bare binary on GitHub releases (scip-ruby, scip-clang).
    GitHubBinary { asset_pattern: String },
    /// Gzipped single binary (rust-analyzer on unix).
    GitHubGz { asset_pattern: String },
    /// Tarball with binary inside (scip-go via goreleaser).
    GitHubTarGz {
        asset_pattern: String,
        binary_path_in_archive: Option<String>,
    },
    /// Zip archive (rust-analyzer on Windows).
    GitHubZip {
        asset_pattern: String,
        binary_path_in_archive: Option<String>,
    },
    /// Coursier launcher script (scip-java).
    GitHubLauncher {
        unix_asset: String,
        windows_asset: String,
    },
    /// npm global package (scip-typescript, scip-python).
    Npm { package: String },
    /// dotnet global tool (scip-dotnet).
    DotnetTool { package: String },
    /// Not independently installable (scip-kotlin via scip-java).
    Unsupported { reason: String },
}

/// Metadata about a SCIP indexer binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerEntry {
    /// Human-readable indexer name (e.g., "scip-typescript")
    pub indexer_name: String,
    /// Language this indexer targets
    pub language: String,
    /// GitHub owner/repo for release downloads
    pub github_repo: String,
    /// Binary name inside the release archive
    pub binary_name: String,
    /// Latest known version tag
    pub version: String,
    /// Additional CLI arguments to pass when invoking
    pub default_args: Vec<String>,
    /// The output filename the indexer produces
    pub output_file: String,
    /// How to install this indexer
    pub install_method: InstallMethod,
}

impl IndexerEntry {
    pub fn language_name(&self) -> &str {
        &self.language
    }

    pub fn binary_name(&self) -> &str {
        &self.binary_name
    }

    /// Check if this indexer is already installed locally.
    pub fn is_installed(&self) -> bool {
        self.installed_path().is_some()
    }

    /// Get the path to the installed binary, if present.
    ///
    /// Checks in order:
    /// 1. Local install directory (install_dir/{binary_name})
    /// 2. Method-specific paths (npm node_modules, dotnet tools)
    /// 3. System PATH via `which`
    pub fn installed_path(&self) -> Option<PathBuf> {
        // 1. Check local install dir
        let dir = install_dir();
        let candidates = if cfg!(windows) {
            vec![
                dir.join(format!("{}.exe", self.binary_name)),
                dir.join(&self.binary_name),
            ]
        } else {
            vec![dir.join(&self.binary_name)]
        };
        for path in &candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }

        // 2. Check method-specific install locations
        match &self.install_method {
            InstallMethod::Npm { .. } => {
                let npm_dir = dir.join("npm").join("node_modules").join(".bin");
                let name = if cfg!(windows) {
                    format!("{}.cmd", self.binary_name)
                } else {
                    self.binary_name.clone()
                };
                let path = npm_dir.join(&name);
                if path.exists() {
                    return Some(path);
                }
            }
            InstallMethod::DotnetTool { .. } => {
                let dotnet_dir = dir.join("dotnet-tools");
                let name = if cfg!(windows) {
                    format!("{}.exe", self.binary_name)
                } else {
                    self.binary_name.clone()
                };
                let path = dotnet_dir.join(&name);
                if path.exists() {
                    return Some(path);
                }
            }
            _ => {}
        }

        // 3. Check system PATH
        which::which(&self.binary_name).ok()
    }

    /// Get the installed version string, if available.
    pub fn installed_version(&self) -> Option<String> {
        if self.is_installed() {
            Some(self.version.clone())
        } else {
            None
        }
    }

    /// Ensure the indexer binary is installed, downloading if necessary.
    pub async fn ensure_installed(&self, progress: &dyn ProgressHandler) -> Result<PathBuf> {
        if let Some(path) = self.installed_path() {
            tracing::debug!(indexer = %self.indexer_name, ?path, "already installed");
            return Ok(path);
        }

        progress.on_event(ProgressEvent::DownloadStart {
            indexer: self.indexer_name.clone(),
            version: self.version.clone(),
        });
        let path = install::install_indexer(self, progress).await?;
        progress.on_event(ProgressEvent::DownloadComplete {
            indexer: self.indexer_name.clone(),
            path: path.clone(),
        });
        Ok(path)
    }
}

/// Return the directory where indexer binaries are stored.
pub fn install_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("scip-io").join("bin");
    std::fs::create_dir_all(&dir).ok();
    dir
}
