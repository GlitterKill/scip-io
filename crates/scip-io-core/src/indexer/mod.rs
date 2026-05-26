pub mod install;
pub mod registry;
pub mod runner;
pub mod version;

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
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
    /// Logical indexer row whose install/run action is provided by another indexer.
    CoveredBy {
        indexer_name: String,
        reason: String,
    },
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedInstallMetadata {
    version: String,
    path: String,
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

    /// Check if this indexer has a built-in automatic install method.
    pub fn is_installable(&self) -> bool {
        !matches!(
            self.install_method,
            InstallMethod::CoveredBy { .. } | InstallMethod::Unsupported { .. }
        )
    }

    /// Get the path to the installed binary, if present.
    ///
    /// Checks in order:
    /// 1. Local install directory (install_dir/{binary_name})
    /// 2. Method-specific paths (npm node_modules, dotnet tools)
    /// 3. System PATH via `which`
    pub fn installed_path(&self) -> Option<PathBuf> {
        let dir = install_dir();
        self.installed_path_from(&dir, true)
    }

    fn installed_path_from(&self, dir: &Path, include_system_path: bool) -> Option<PathBuf> {
        // 1. Check local install dir
        for path in self.local_binary_candidates(dir) {
            if path.exists() {
                return Some(path);
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
        if include_system_path {
            which::which(&self.binary_name).ok()
        } else {
            None
        }
    }

    /// Return true when the discovered binary lives inside SCIP-IO's managed
    /// install cache. System PATH binaries are intentionally excluded because
    /// the app did not install them and should not remove them.
    pub fn is_managed_installed(&self) -> bool {
        self.installed_path()
            .as_deref()
            .is_some_and(is_managed_install_path)
    }

    /// Remove the app-managed install files for this indexer.
    ///
    /// This removes enough method-specific files to make `installed_path`
    /// return `None` for SCIP-IO-managed installs. It refuses to remove
    /// binaries found only on the user's PATH.
    pub fn uninstall_managed(&self) -> Result<Option<PathBuf>> {
        let dir = install_dir();
        let Some(installed_path) = self.installed_path() else {
            return Ok(None);
        };

        if !is_managed_install_path_in(&installed_path, &dir) {
            bail!(
                "{} is installed at {}, which is outside SCIP-IO's managed cache",
                self.indexer_name,
                installed_path.display()
            );
        }

        self.remove_managed_install_from(&dir, installed_path)
    }

    #[cfg(test)]
    fn uninstall_managed_from(&self, dir: &Path) -> Result<Option<PathBuf>> {
        let Some(installed_path) = self.installed_path_from(dir, false) else {
            return Ok(None);
        };

        self.remove_managed_install_from(dir, installed_path)
    }

    fn remove_managed_install_from(
        &self,
        dir: &Path,
        installed_path: PathBuf,
    ) -> Result<Option<PathBuf>> {
        let mut removed_any = false;
        for path in self.managed_removal_candidates(dir) {
            removed_any |= remove_candidate_path(&path)?;
        }

        if !removed_any {
            remove_candidate_path(&installed_path)?;
        }

        Ok(Some(installed_path))
    }

    /// Get the installed version string, if available.
    pub fn installed_version(&self) -> Option<String> {
        let installed_path = self.installed_path()?;
        if is_managed_install_path(&installed_path) {
            return self
                .managed_install_metadata()
                .map(|metadata| metadata.version)
                .or_else(|| Some(self.version.clone()));
        }

        Some(self.version.clone())
    }

    /// Ensure the indexer binary is installed, downloading if necessary.
    pub async fn ensure_installed(&self, progress: &dyn ProgressHandler) -> Result<PathBuf> {
        if let Some(path) = self.installed_path() {
            install::repair_existing_indexer(self)?;
            if is_managed_install_path(&path) && self.managed_install_metadata().is_none() {
                self.write_managed_install_metadata(&self.version, &path)?;
            }
            tracing::debug!(indexer = %self.indexer_name, ?path, "already installed");
            return Ok(path);
        }

        let version = install::resolve_latest_compatible_version(self).await?;
        self.install_version(&version, progress).await
    }

    pub async fn install_version(
        &self,
        version: &str,
        progress: &dyn ProgressHandler,
    ) -> Result<PathBuf> {
        progress.on_event(ProgressEvent::DownloadStart {
            indexer: self.indexer_name.clone(),
            version: version.to_owned(),
        });
        let path = install::install_indexer_at_version(self, version, progress).await?;
        self.write_managed_install_metadata(version, &path)?;
        progress.on_event(ProgressEvent::DownloadComplete {
            indexer: self.indexer_name.clone(),
            path: path.clone(),
        });
        Ok(path)
    }

    pub async fn update_managed_to_version(
        &self,
        version: &str,
        progress: &dyn ProgressHandler,
    ) -> Result<PathBuf> {
        let Some(installed_path) = self.installed_path() else {
            bail!("{} is not installed", self.indexer_name);
        };

        progress.on_event(ProgressEvent::DownloadStart {
            indexer: self.indexer_name.clone(),
            version: version.to_owned(),
        });

        if !is_managed_install_path(&installed_path) {
            bail!(
                "{} is installed at {}, which is outside SCIP-IO's managed cache",
                self.indexer_name,
                installed_path.display()
            );
        }

        let dir = install_dir();
        let backup_dir = dir.join("update-backups").join(format!(
            "{}-{}",
            self.indexer_name,
            std::process::id()
        ));
        let candidates = self.managed_removal_candidates(&dir);
        backup_paths(&dir, &candidates, &backup_dir)?;
        self.remove_managed_install_from(&dir, installed_path)?;
        let path = match install::install_indexer_at_version(self, version, progress).await {
            Ok(path) => {
                remove_candidate_path(&backup_dir).ok();
                path
            }
            Err(err) => {
                restore_backup_paths(&dir, &backup_dir)?;
                remove_candidate_path(&backup_dir).ok();
                return Err(err.context(format!(
                    "failed to update {}; restored previous managed install",
                    self.indexer_name
                )));
            }
        };
        self.write_managed_install_metadata(version, &path)?;

        progress.on_event(ProgressEvent::DownloadComplete {
            indexer: self.indexer_name.clone(),
            path: path.clone(),
        });
        Ok(path)
    }

    fn local_binary_candidates(&self, dir: &Path) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if cfg!(windows) {
            candidates.push(dir.join(format!("{}.exe", self.binary_name)));
            candidates.push(dir.join(&self.binary_name));
            if matches!(self.install_method, InstallMethod::GitHubLauncher { .. }) {
                candidates.push(dir.join(format!("{}.bat", self.binary_name)));
            }
        } else {
            candidates.push(dir.join(&self.binary_name));
        }
        candidates
    }

    fn managed_removal_candidates(&self, dir: &Path) -> Vec<PathBuf> {
        let mut candidates = self.local_binary_candidates(dir);
        candidates.push(self.metadata_path_in(dir));

        match &self.install_method {
            InstallMethod::Npm { package } => {
                let npm_root = dir.join("npm");
                let bin_dir = npm_root.join("node_modules").join(".bin");
                candidates.push(npm_package_dir(&npm_root, package));
                candidates.push(bin_dir.join(&self.binary_name));
                candidates.push(bin_dir.join(format!("{}.cmd", self.binary_name)));
                candidates.push(bin_dir.join(format!("{}.ps1", self.binary_name)));
            }
            InstallMethod::DotnetTool { .. } => {
                let dotnet_dir = dir.join("dotnet-tools");
                candidates.push(dotnet_dir.join(&self.binary_name));
                candidates.push(dotnet_dir.join(format!("{}.exe", self.binary_name)));
            }
            _ => {}
        }

        candidates
    }

    pub(crate) fn write_managed_install_metadata(&self, version: &str, path: &Path) -> Result<()> {
        let dir = install_dir();
        self.write_managed_install_metadata_in(&dir, version, path)
    }

    fn managed_install_metadata(&self) -> Option<ManagedInstallMetadata> {
        let dir = install_dir();
        self.managed_install_metadata_from(&dir)
    }

    fn managed_install_metadata_from(&self, dir: &Path) -> Option<ManagedInstallMetadata> {
        let path = self.metadata_path_in(dir);
        let raw = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write_managed_install_metadata_in(
        &self,
        dir: &Path,
        version: &str,
        path: &Path,
    ) -> Result<()> {
        let metadata = ManagedInstallMetadata {
            version: version.to_owned(),
            path: path.to_string_lossy().to_string(),
        };
        let metadata_path = self.metadata_path_in(dir);
        if let Some(parent) = metadata_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
        Ok(())
    }

    fn metadata_path_in(&self, dir: &Path) -> PathBuf {
        dir.join("metadata")
            .join(format!("{}.json", self.indexer_name))
    }
}

pub(crate) fn npm_package_dir(prefix_dir: &Path, package: &str) -> PathBuf {
    package
        .split('/')
        .fold(prefix_dir.join("node_modules"), |path, segment| {
            path.join(segment)
        })
}

pub fn is_managed_install_path(path: &Path) -> bool {
    let dir = install_dir();
    is_managed_install_path_in(path, &dir)
}

fn is_managed_install_path_in(path: &Path, dir: &Path) -> bool {
    let Some(root) = dir.canonicalize().ok() else {
        return false;
    };
    let Some(path) = path.canonicalize().ok() else {
        return false;
    };
    path.starts_with(root)
}

fn remove_candidate_path(path: &Path) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };

    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }

    Ok(true)
}

fn backup_paths(root: &Path, candidates: &[PathBuf], backup_root: &Path) -> Result<()> {
    remove_candidate_path(backup_root).ok();
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        if !is_managed_install_path_in(candidate, root) {
            continue;
        }
        let relative = candidate.strip_prefix(root)?;
        let dest = backup_root.join(relative);
        copy_path(candidate, &dest)?;
    }
    Ok(())
}

fn restore_backup_paths(root: &Path, backup_root: &Path) -> Result<()> {
    if !backup_root.exists() {
        return Ok(());
    }

    for entry in walkdir::WalkDir::new(backup_root).min_depth(1) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(backup_root)?;
        let dest = root.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(entry.path(), dest)?;
    }
    Ok(())
}

fn copy_path(source: &Path, dest: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_dir() {
        for entry in walkdir::WalkDir::new(source) {
            let entry = entry?;
            let relative = entry.path().strip_prefix(source)?;
            let target = dest.join(relative);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&target)?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), target)?;
            }
        }
    } else if metadata.file_type().is_file() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, dest)?;
    }
    Ok(())
}

/// Return the directory where indexer binaries are stored.
pub fn install_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("scip-io").join("bin");
    std::fs::create_dir_all(&dir).ok();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry_with_method(binary_name: &str, install_method: InstallMethod) -> IndexerEntry {
        IndexerEntry {
            indexer_name: binary_name.to_string(),
            language: "test".to_string(),
            github_repo: "owner/repo".to_string(),
            binary_name: binary_name.to_string(),
            version: "1.0.0".to_string(),
            default_args: Vec::new(),
            output_file: "index.scip".to_string(),
            install_method,
        }
    }

    fn npm_entry() -> IndexerEntry {
        entry_with_method(
            "scip-python",
            InstallMethod::Npm {
                package: "@sourcegraph/scip-python".to_string(),
            },
        )
    }

    fn create_file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "binary").unwrap();
    }

    fn managed_binary_path(root: &Path, binary_name: &str) -> PathBuf {
        if cfg!(windows) {
            root.join(format!("{binary_name}.exe"))
        } else {
            root.join(binary_name)
        }
    }

    fn managed_launcher_path(root: &Path, binary_name: &str) -> PathBuf {
        if cfg!(windows) {
            root.join(format!("{binary_name}.bat"))
        } else {
            root.join(binary_name)
        }
    }

    #[test]
    fn scoped_npm_package_dir_uses_node_modules_scope_layout() {
        let root = PathBuf::from("cache").join("npm");

        let package_dir = npm_package_dir(&root, "@sourcegraph/scip-python");

        assert_eq!(
            package_dir,
            root.join("node_modules")
                .join("@sourcegraph")
                .join("scip-python")
        );
    }

    #[test]
    fn npm_uninstall_candidates_include_package_and_all_shims() {
        let root = PathBuf::from("cache");
        let candidates = npm_entry().managed_removal_candidates(&root);

        assert!(
            candidates.contains(
                &root
                    .join("npm")
                    .join("node_modules")
                    .join("@sourcegraph")
                    .join("scip-python")
            )
        );
        assert!(
            candidates.contains(
                &root
                    .join("npm")
                    .join("node_modules")
                    .join(".bin")
                    .join("scip-python")
            )
        );
        assert!(
            candidates.contains(
                &root
                    .join("npm")
                    .join("node_modules")
                    .join(".bin")
                    .join("scip-python.cmd")
            )
        );
        assert!(
            candidates.contains(
                &root
                    .join("npm")
                    .join("node_modules")
                    .join(".bin")
                    .join("scip-python.ps1")
            )
        );
    }

    #[test]
    fn managed_install_metadata_round_trips_installed_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entry = npm_entry();
        let path = root
            .join("npm")
            .join("node_modules")
            .join(".bin")
            .join("scip-python.cmd");

        entry
            .write_managed_install_metadata_in(root, "0.7.0", &path)
            .unwrap();

        let metadata = entry.managed_install_metadata_from(root).unwrap();
        assert_eq!(metadata.version, "0.7.0");
        assert_eq!(metadata.path, path.to_string_lossy());
    }

    #[test]
    fn uninstall_managed_from_removes_metadata_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entry = npm_entry();
        let binary = root
            .join("npm")
            .join("node_modules")
            .join(".bin")
            .join(if cfg!(windows) {
                "scip-python.cmd"
            } else {
                "scip-python"
            });

        create_file(&binary);
        entry
            .write_managed_install_metadata_in(root, "0.7.0", &binary)
            .unwrap();

        let metadata_path = entry.metadata_path_in(root);
        assert!(metadata_path.exists());

        entry.uninstall_managed_from(root).unwrap();

        assert!(!metadata_path.exists());
    }

    #[test]
    fn uninstall_managed_from_removes_npm_package_and_shims() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entry = npm_entry();
        let package_dir = root
            .join("npm")
            .join("node_modules")
            .join("@sourcegraph")
            .join("scip-python");
        let bin_dir = root.join("npm").join("node_modules").join(".bin");
        let shims = [
            bin_dir.join("scip-python"),
            bin_dir.join("scip-python.cmd"),
            bin_dir.join("scip-python.ps1"),
        ];

        create_file(&package_dir.join("package.json"));
        for shim in &shims {
            create_file(shim);
        }

        let removed = entry.uninstall_managed_from(root).unwrap();

        assert!(removed.is_some());
        assert!(!package_dir.exists());
        for shim in &shims {
            assert!(!shim.exists(), "expected {} to be removed", shim.display());
        }
        assert!(entry.installed_path_from(root, false).is_none());
    }

    #[test]
    fn uninstall_managed_from_removes_dotnet_tool_binary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entry = entry_with_method(
            "scip-dotnet",
            InstallMethod::DotnetTool {
                package: "scip-dotnet".to_string(),
            },
        );
        let tool_path = if cfg!(windows) {
            root.join("dotnet-tools").join("scip-dotnet.exe")
        } else {
            root.join("dotnet-tools").join("scip-dotnet")
        };
        create_file(&tool_path);

        let removed = entry.uninstall_managed_from(root).unwrap();

        assert_eq!(removed.as_deref(), Some(tool_path.as_path()));
        assert!(!tool_path.exists());
        assert!(entry.installed_path_from(root, false).is_none());
    }

    #[test]
    fn uninstall_managed_from_removes_github_binary_and_archive_binaries() {
        let methods = [
            InstallMethod::GitHubBinary {
                asset_pattern: "tool-{os}".to_string(),
            },
            InstallMethod::GitHubGz {
                asset_pattern: "tool-{os}.gz".to_string(),
            },
            InstallMethod::GitHubTarGz {
                asset_pattern: "tool-{os}.tar.gz".to_string(),
                binary_path_in_archive: None,
            },
            InstallMethod::GitHubZip {
                asset_pattern: "tool-{os}.zip".to_string(),
                binary_path_in_archive: None,
            },
        ];

        for (index, method) in methods.into_iter().enumerate() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let binary_name = format!("github-tool-{index}");
            let entry = entry_with_method(&binary_name, method);
            let binary_path = managed_binary_path(root, &binary_name);
            create_file(&binary_path);

            let removed = entry.uninstall_managed_from(root).unwrap();

            assert_eq!(removed.as_deref(), Some(binary_path.as_path()));
            assert!(!binary_path.exists());
            assert!(entry.installed_path_from(root, false).is_none());
        }
    }

    #[test]
    fn uninstall_managed_from_removes_launcher_script() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let entry = entry_with_method(
            "scip-java",
            InstallMethod::GitHubLauncher {
                unix_asset: "scip-java-{version}".to_string(),
                windows_asset: "scip-java-{version}.bat".to_string(),
            },
        );
        let launcher_path = managed_launcher_path(root, "scip-java");
        create_file(&launcher_path);

        let removed = entry.uninstall_managed_from(root).unwrap();

        assert_eq!(removed.as_deref(), Some(launcher_path.as_path()));
        assert!(!launcher_path.exists());
        assert!(entry.installed_path_from(root, false).is_none());
    }

    #[test]
    fn managed_path_check_rejects_sibling_cache_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let inside = root.join("tool");
        let sibling = temp.path().join("cache-other").join("tool");
        create_file(&inside);
        create_file(&sibling);

        assert!(is_managed_install_path_in(&inside, &root));
        assert!(!is_managed_install_path_in(&sibling, &root));
    }

    #[test]
    fn update_backup_restores_removed_files_and_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let backup = temp.path().join("backup");
        let binary = root.join("tool.exe");
        let package_file = root
            .join("npm")
            .join("node_modules")
            .join("pkg")
            .join("package.json");

        create_file(&binary);
        create_file(&package_file);

        backup_paths(
            &root,
            &[binary.clone(), package_file.parent().unwrap().to_path_buf()],
            &backup,
        )
        .unwrap();
        remove_candidate_path(&binary).unwrap();
        remove_candidate_path(package_file.parent().unwrap()).unwrap();

        restore_backup_paths(&root, &backup).unwrap();

        assert!(binary.exists());
        assert!(package_file.exists());
    }
}
